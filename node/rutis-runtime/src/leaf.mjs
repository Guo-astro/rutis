// Leaf plugins, as the SDK (@arcships/rutis) makes them: a JavaScript or
// TypeScript plugin that needs no Cordis. The runtime loads it into its
// Cordis Context like any plugin, so it shares services — and the logical
// event bus (./cordis-events.mjs) — with Cordis plugins in the same
// process; the plugin itself only sees `ctx.use`, `ctx.get`, `ctx.provide`,
// `ctx.effect`, `ctx.signal`, `ctx.on` and the five ways to emit (`emit`,
// `parallel`, `serial`, `waterfall`, `bail`), with `ctx.filter`.
// The runtime does not import the SDK: it recognises its plugins by the
// symbol the SDK marks them with.
import { KEEP, LEAF_FIBERS, LEAF_NAME } from './cordis-events.mjs'
import { refuseReserved } from './events.mjs'
import { UNDECLARED, notDeclared, notDeclaredOptional, unavailable } from './errors.mjs'

// The plugin API this runtime runs: a plugin written against a newer one
// is refused, naming both. 3: the unified events of rutis 0.13 (#464).
export const PLUGIN_API = 3

const PLUGIN = Symbol.for('rutis.leaf-plugin')

export const isLeaf = value => value?.[PLUGIN] === true

// Refuse a plugin that needs a newer plugin API than this runtime's.
export function supported(plugin, name) {
  if (plugin.api > PLUGIN_API) {
    throw new Error(`plugin ${name} needs plugin API ${plugin.api}; this runtime supports ${PLUGIN_API}: upgrade @arcships/rutis-runtime where the host runs`)
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

// What a leaf plugin using a service that neither its `inject` nor its
// row's declares, and that it does not provide itself, is told whether or
// not the service is there: not that it is missing, but what to declare
// (#676). The same in every runtime
// and testing tool (./errors.mjs).
export { UNDECLARED }

// What a leaf plugin providing a service someone else provides there is
// told: `forecast is already provided by a` (§8.1 row 13).
export const ALREADY_PROVIDED = (name, provider) => `${name} is already provided by ${provider}`

// Marks the test driver (./testing/rutis-test.mjs): it is the test, not a
// plugin under the rules, and uses whatever service the test names,
// declared or not, as Python's test tool reads the runtime's services.
// Not Symbol.for: no plugin outside this package can set it.
export const USES_ANY = Symbol('rutis.uses-any')

// The Cordis plugin that runs a leaf plugin. Events are the Context's:
// `context.on` owns listener cleanup through the plugin's fiber.
// `rowInject`: the names its row's own `inject` adds.
// `providerOf(context, name)`: who already provides `name` there (a row's
// name, or `the host`), to say so when the plugin provides it too.
export function toCordis(spec, rowInject = [], providerOf) {
  // What the plugin may `use`: its `inject` and its row's `inject` (as
  // Rust's `require` reads only the injects); the test driver, anything.
  // It may `get` its `optional` too.
  const usesAny = spec[USES_ANY] === true
  const uses = name => usesAny || (spec.inject ?? []).includes(name) || rowInject.includes(name)
  return {
    name: spec.name ?? 'leaf',
    inject: spec.inject,
    async apply(context, config) {
      LEAF_FIBERS.add(context.fiber)
      // What it provides itself is declared too, while it provides it: a
      // plugin reads what it provides (as in every other runtime). Kept
      // here, each entry until the function its provide returned is called:
      // Cordis's get does not see what a loading fiber provides, and its
      // fiber store drops a withdrawn name only some microtasks later, after
      // the plugin may have provided it again. (`context[name]` is no way
      // either: Cordis answers names such as `fiber`, `then` or `_u` itself.)
      const provided = new Map()
      const mayUse = name => uses(name) || provided.has(name)
      const mayGet = name => mayUse(name) || (spec.optional ?? []).includes(name)
      const read = name => provided.has(name) ? provided.get(name).value : context.get(name)
      const ctx = {
        use(name) {
          // One neither the plugin nor its row injects (an optional one is
          // read with `get`): say what to declare, not that it is missing
          // (#676), whether or not another row here happens to provide it.
          if (!mayUse(name)) throw notDeclared(name)
          const value = read(name)
          if (value === undefined) throw unavailable(name)
          return value
        },
        // An optional service: the service when there, else undefined.
        get(name) {
          if (!mayGet(name)) throw notDeclaredOptional(name)
          return read(name)
        },
        provide: (name, value) => {
          let withdraw
          try {
            withdraw = context.provide(name, value)
          } catch (error) {
            // Cordis's words name its internal fibers (`<root>`): say who.
            if (providerOf && /^service ".*" has been registered at <.*>$/.test(error?.message)) throw new Error(ALREADY_PROVIDED(name, providerOf(context, name)))
            throw error
          }
          const entry = { value }
          provided.set(name, entry)
          return () => {
            // Only this provide's entry: the name may be provided again.
            if (provided.get(name) === entry) provided.delete(name)
            return withdraw()
          }
        },
        effect: cleanup => { context.effect(() => cleanup) },
        // The listener leaves with the plugin, or when the returned function
        // is called. `prepend` and `global` as Cordis's. Descriptors name the
        // plugin by its declared name, else by its row's id.
        on: (name, listener, options) => {
          const off = context.on(reserved(name), listener, { prepend: Boolean(options?.prepend), global: Boolean(options?.global), [LEAF_NAME]: spec.name ?? null })
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
      // The config as the host checked it against the plugin's schema, its
      // defaults filled (the kernel's `rutis::config::check`, platform
      // cleanup §7.5): the runtime only passes it on.
      const cleanup = await spec.apply(ctx, config)
      if (cleanup !== undefined && cleanup !== null) {
        if (typeof cleanup !== 'function') throw new TypeError('apply must return a cleanup function or nothing')
        context.effect(() => cleanup)
      }
    },
  }
}

// The five ways to emit, from `context`, with `thisArg` (a filter) when
// given. Every argument of `waterfall` is the event's: a terminal is given
// with `waterfall.with(terminal)(name, ...args)` (Cordis's own waterfall
// takes it as its last argument, which a leaf plugin, whose arguments may
// be functions, cannot tell apart).
function emitters(context, thisArg) {
  const args = (name, values) => thisArg ? [thisArg, emittable(name), ...values] : [emittable(name), ...values]
  const waterfall = terminal => (name, ...values) => context.waterfall(...args(name, values), terminal)
  return {
    emit: (name, ...values) => { context.emit(...args(name, values)) },
    parallel: (name, ...values) => context.parallel(...args(name, values)),
    serial: (name, ...values) => context.serial(...args(name, values)),
    waterfall: Object.assign(waterfall(undefined), { with: terminal => {
      if (typeof terminal !== 'function') throw new TypeError('waterfall.with needs a terminal function')
      return waterfall(terminal)
    } }),
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
