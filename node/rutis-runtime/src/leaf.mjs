// Leaf plugins, as the SDK (@arcships/rutis) makes them: a JavaScript or
// TypeScript plugin that needs no Cordis. The runtime loads it into its
// Cordis Context like any plugin, so it shares services — and the logical
// event bus (./cordis-events.mjs) — with Cordis plugins in the same
// process; the plugin itself only sees `ctx.use`, `ctx.provide`,
// `ctx.effect`, `ctx.signal`, `ctx.on` and the five ways to emit (`emit`,
// `parallel`, `serial`, `waterfall`, `bail`), with `ctx.filter`.
// The runtime does not import the SDK: it recognises its plugins by the
// symbol the SDK marks them with.
import { KEEP } from './cordis-events.mjs'
import { refuseReserved } from './events.mjs'

// The plugin APIs this runtime runs: a plugin written against a newer one
// is refused, naming both.
// 2: ctx.on and ctx.emit (leaf events). A runtime that supports only 1
// refuses such a plugin at load instead of failing in apply.
// 3: the unified events of rutis 0.13 (#464): ctx.parallel, serial,
// waterfall, bail and filter, and ctx.signal. Plugins of API 2 still load.
export const PLUGIN_API = 3

const PLUGIN = Symbol.for('rutis.leaf-plugin')

export const isLeaf = value => value?.[PLUGIN] === true

// Refuse a plugin that needs a newer plugin API than this runtime's.
export function supported(plugin, name) {
  const api = plugin?.api ?? 1
  if (api > PLUGIN_API) {
    throw new Error(`plugin ${name} needs plugin API ${api}; this runtime supports ${PLUGIN_API}: upgrade @arcships/rutis-runtime where the host runs`)
  }
  return plugin
}

// Cordis's internal/* events drive the runner itself and do not exist on the
// other runtimes, so a leaf plugin can neither listen to nor emit them
// (internal/* stays in each runtime, design §7). This is the one place the
// Node side defines the check: the leaf adapter and the test tool use the
// wording here (`node/rutis/src/testing.mjs` repeats it, since the SDK is
// its own package and cannot import the runtime). A name is a string:
// Cordis's `emit` takes an object, null or function first argument as
// `this`, so any other type would let `emit(null, 'internal/service')` past
// the check.
export const INTERNAL_MESSAGE = name => `event ${name} is internal to the runtime`

export function reserved(name) {
  if (typeof name !== 'string') throw new TypeError(`an event name is a string, not ${name === null ? 'null' : typeof name}`)
  if (name.startsWith('internal/')) throw new Error(INTERNAL_MESSAGE(name))
  return name
}

// A name a leaf plugin emits: not internal/*, and not rutis/*
// (ReservedEvent, refused synchronously by every emitting method).
function emittable(name) {
  reserved(name)
  refuseReserved(name)
  return name
}

// What a leaf plugin providing a service its `provides` does not declare
// is told when the host, older than 0.12, mounted the runtime without
// `announce`: such a host does not take the service, which stays in this
// process.
export const OLD_HOST = name => `rutis: service ${name} is not declared in provides, and this host (older than 0.12) does not take undeclared services; declare it or upgrade rutis-host`

// A function telling OLD_HOST once per name, with `write`.
export function oncePerName(write) {
  const told = new Set()
  return name => {
    if (told.has(name)) return
    told.add(name)
    write(`${OLD_HOST(name)}\n`)
  }
}

// The Cordis plugin that runs a leaf plugin. Events are the Context's:
// `context.on` owns listener cleanup through the plugin's fiber.
// `undeclared(name)`: the plugin provides `name`, which its `provides` does
// not declare.
export function toCordis(spec, undeclared) {
  return {
    name: spec.name ?? 'leaf',
    inject: spec.inject,
    async apply(context, config) {
      const ctx = {
        use(name) {
          const value = context.get(name)
          if (value === undefined) throw new Error(`service ${name} is not available`)
          return value
        },
        provide: (name, value) => {
          // A keyed name (`#`) is never announced, by any host.
          if (undeclared && !name.includes('#') && !Object.hasOwn(spec.provides ?? {}, name)) undeclared(name)
          return context.provide(name, value)
        },
        effect: cleanup => { context.effect(() => cleanup) },
        // The listener leaves with the plugin, or when the returned function
        // is called. `prepend` and `global` as Cordis's.
        on: (name, listener, options) => {
          const off = context.on(reserved(name), listener, { prepend: Boolean(options?.prepend), global: Boolean(options?.global) })
          return () => { off() }
        },
        ...emitters(context, null),
        filter: predicate => {
          if (typeof predicate !== 'function') throw new TypeError('filter needs a predicate function')
          return emitters(context, { [KEEP]: descriptor => predicate(descriptor) })
        },
        // Aborted when the row unloads, while its listeners are waited for.
        get signal() { return signalOf(context) },
      }
      const cleanup = await spec.apply(ctx, config)
      if (cleanup !== undefined && cleanup !== null) {
        if (typeof cleanup !== 'function') throw new TypeError('apply must return a cleanup function or nothing')
        context.effect(() => cleanup)
      }
    },
  }
}

// The five ways to emit, from `context`, with `thisArg` (a filter) when
// given. `waterfall`'s last argument is its terminal.
function emitters(context, thisArg) {
  const args = (name, values) => thisArg ? [thisArg, emittable(name), ...values] : [emittable(name), ...values]
  return {
    emit: (name, ...values) => { context.emit(...args(name, values)) },
    parallel: (name, ...values) => context.parallel(...args(name, values)),
    serial: (name, ...values) => context.serial(...args(name, values)),
    waterfall: (name, ...values) => context.waterfall(...args(name, values)),
    bail: (name, ...values) => context.bail(...args(name, values)),
  }
}

// The row's signal, from the runtime (`ctx.signal` of its Context); a
// Context without one (a Cordis application hosting the plugin) gets one
// aborted when the plugin's fiber goes.
const signals = new WeakMap()
function signalOf(context) {
  try {
    const signal = context.signal
    if (signal) return signal
  } catch {}
  const fiber = context.fiber
  let controller = signals.get(fiber)
  if (!controller) {
    signals.set(fiber, controller = new AbortController())
    context.effect(() => () => controller.abort())
  }
  return controller.signal
}
