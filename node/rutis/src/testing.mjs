// Test a plugin without a host: give it the services it injects, call the
// services it provides, unload it.
//
//   const t = await load(plugin, { config: { city: 'Oslo' }, services: { llm } })
//   assert.equal(await t.service('weather').today(), 'sunny in Oslo')
//   await t.unload()
//
// It checks what a host would: the plugin uses only the services it declares
// in `inject`, provides what it declares in `provides` with every declared
// method, and runs its cleanups on unload. With `strict` (the default),
// values cross between the plugin and the test as they would between
// processes: data is copied, functions and objects with behaviour pass by
// reference, sync methods return values and async ones promises; what
// works only in one process fails here too.

import { Context } from '@deepseek-ai/cordis'
import { PLUGIN_API, isPlugin } from './index.mjs'

export class PluginTestError extends Error {
  name = 'PluginTestError'
}

const builtins = [Date, RegExp, Map, Set, WeakMap, WeakSet, ArrayBuffer, DataView, Error, Promise]
// The Errors a thrown error keeps its class of, as the runtimes decode them.
const constructors = { Error, TypeError, RangeError, ReferenceError, SyntaxError, URIError, EvalError }

// Objects with behaviour cross by reference: class instances and objects
// with methods. Everything else is data.
function hasBehaviour(value) {
  if (value === null || typeof value !== 'object' || Array.isArray(value) || ArrayBuffer.isView(value)) return false
  if (builtins.some(type => value instanceof type)) return false
  const prototype = Object.getPrototypeOf(value)
  if (prototype !== Object.prototype && prototype !== null) return true
  return Object.values(value).some(item => typeof item === 'function')
}

// Whether a value holds a reference anywhere: itself, or inside the arrays
// and plain objects it contains. A plain object that holds none is plain
// data (JSON), where undefined becomes null.
function holdsReference(value, seen = new Set()) {
  if (typeof value === 'function' || value instanceof Promise || hasBehaviour(value)) return true
  if (value === null || typeof value !== 'object' || seen.has(value)) return false
  if (!Array.isArray(value) && ![Object.prototype, null].includes(Object.getPrototypeOf(value))) return false
  seen.add(value)
  return Object.values(value).some(item => holdsReference(item, seen))
}

// How deep a value crossing between processes may nest, as in the
// runtimes (#279): each array or object is a level.
const MAX_DEPTH = 64

// `value` as the other process sees it, by the rules of the runtimes
// (node/rutis-runtime/src/session.mjs); `data`: inside plain data; `depth`:
// the arrays and objects around it.
function cross(value, where, seen = new Set(), data = false, depth = 0) {
  if (value === undefined) return data ? null : undefined
  if (value === null) return value
  if (typeof value === 'symbol' || typeof value === 'bigint') {
    throw new PluginTestError(`${where}: a ${typeof value} cannot cross between processes`)
  }
  if (typeof value === 'number' && !Number.isFinite(value)) {
    throw new PluginTestError(`${where}: ${value} cannot cross between processes (only finite numbers do)`)
  }
  if (typeof value === 'function') {
    return (...args) => crossResult(() => value(...args.map((arg, i) => cross(arg, `${where} argument ${i}`))), where)
  }
  if (value instanceof Promise) return value.then(result => cross(result, where), thrown => { throw crossThrown(thrown, where) })
  if (typeof value !== 'object') return value
  if (hasBehaviour(value)) return reference(value, where)
  if (seen.has(value)) throw new PluginTestError(`${where}: cyclic data cannot cross between processes`)
  if (depth >= MAX_DEPTH) throw new PluginTestError(`${where}: a value nested deeper than ${MAX_DEPTH} levels cannot cross between processes`)
  // An Error passed as a value crosses as data: its name, message and stack.
  if (value instanceof Error && !data) {
    return { name: value.name, message: value.message, ...(typeof value.stack === 'string' ? { stack: value.stack } : {}) }
  }
  if (!Array.isArray(value) && ![Object.prototype, null].includes(Object.getPrototypeOf(value))) {
    const kind = value.constructor?.name ?? 'object'
    throw new PluginTestError(`${where}: a ${kind} cannot cross between processes (only plain data, functions and objects with methods do)`)
  }
  seen.add(value)
  try {
    if (Array.isArray(value)) return value.map((item, i) => cross(item, `${where}[${i}]`, seen, data, depth + 1))
    const inData = data || !holdsReference(value)
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, cross(item, `${where}.${key}`, seen, inData, depth + 1)]))
  } finally {
    seen.delete(value)
  }
}

// Call `make` from the other process: its result crosses back, and so does
// what it throws.
function crossResult(make, where) {
  let result
  try {
    result = make()
  } catch (thrown) {
    throw crossThrown(thrown, where)
  }
  return result instanceof Promise
    ? result.then(value => cross(value, where), thrown => { throw crossThrown(thrown, where) })
    : cross(result, where)
}

// A thrown value as the other process receives it: an Error is copied with
// its name, message, stack, cause and (AggregateError) errors, as an Error
// of the same built-in class or a plain Error; its other fields stay
// behind. Other thrown values are copied.
function crossThrown(thrown, where, copies = new Map()) {
  if (thrown instanceof PluginTestError) return thrown
  if (thrown === null || typeof thrown !== 'object') {
    if (typeof thrown === 'function' || typeof thrown === 'symbol') {
      return new PluginTestError(`${where}: threw a ${typeof thrown}, which cannot cross between processes`)
    }
    return thrown
  }
  if (copies.has(thrown)) return copies.get(thrown)
  if (thrown instanceof Error) {
    const copy = thrown instanceof AggregateError
      ? new AggregateError([], thrown.message)
      : new (Object.hasOwn(constructors, thrown.name) ? constructors[thrown.name] : Error)(thrown.message)
    Object.assign(copy, { name: thrown.name }, typeof thrown.stack === 'string' ? { stack: thrown.stack } : {})
    copies.set(thrown, copy)
    if ('cause' in thrown) {
      Object.defineProperty(copy, 'cause', { value: crossThrown(thrown.cause, where, copies), configurable: true, writable: true })
    }
    if (thrown instanceof AggregateError) copy.errors = [...thrown.errors].map(item => crossThrown(item, where, copies))
    return copy
  }
  const copy = Array.isArray(thrown) ? [] : {}
  copies.set(thrown, copy)
  for (const [key, item] of Object.entries(thrown)) copy[key] = crossThrown(item, where, copies)
  return copy
}

// An object with behaviour, used from the other process: its methods are
// called by reference.
function reference(target, where) {
  return new Proxy(target, {
    get(object, property) {
      const value = Reflect.get(object, property, object)
      return typeof value === 'function' ? cross(value.bind(object), `${where}.${String(property)}`) : cross(value, `${where}.${String(property)}`)
    },
  })
}

// A service seen through its declared shape: only declared methods, sync
// ones returning values, async ones promises.
function shaped(name, object, shape, strict) {
  const service = {}
  for (const [method, kind] of Object.entries(shape)) {
    service[method] = (...args) => {
      const where = `${name}.${method}`
      const crossed = strict ? args.map((arg, i) => cross(arg, `${where} argument ${i}`)) : args
      let result
      try {
        result = object[method](...crossed)
      } catch (thrown) {
        throw strict ? crossThrown(thrown, where) : thrown
      }
      if (kind === 'sync') {
        if (result instanceof Promise) throw new PluginTestError(`${where} is declared sync but returned a promise: declare it 'async'`)
        return strict ? cross(result, where) : result
      }
      if (!strict) return Promise.resolve(result)
      return Promise.resolve(result).then(value => cross(value, where), thrown => { throw crossThrown(thrown, where) })
    }
  }
  return service
}

// As on the real runtimes, internal/* events belong to the runtime. The SDK
// is a package of its own, so it keeps its own copy of the wording the
// runtime uses (`node/rutis-runtime/src/leaf.mjs`).
const internalMessage = name => `event ${name} is internal to the runtime`

function internal(name) {
  if (typeof name === 'string' && name.startsWith('internal/')) throw new PluginTestError(internalMessage(name))
}

/**
 * Load `plugin` (what definePlugin returns, or a module whose default export
 * it is) with `config` and the services in `services`. `emits` lists the
 * events `t.emit` may send, as `RuntimePlugin::emits` does on a real runtime.
 */
export async function load(plugin, { config = {}, services = {}, strict = true, emits = [] } = {}) {
  plugin = isPlugin(plugin) ? plugin : plugin?.default
  if (!isPlugin(plugin)) throw new PluginTestError('load needs a plugin made with definePlugin')
  if (plugin.api > PLUGIN_API) {
    throw new PluginTestError(`the plugin needs plugin API ${plugin.api}; this SDK supports ${PLUGIN_API}`)
  }
  if (!Array.isArray(emits) || emits.some(name => typeof name !== 'string')) throw new PluginTestError('emits must be a list of event names')
  // As the runners' mount: internal/* cannot be declared for emission.
  for (const name of emits) {
    if (name.startsWith('internal/')) throw new PluginTestError(`${internalMessage(name)} and cannot be emitted from rutis`)
  }
  const declared = new Set(emits)
  for (const name of plugin.inject) {
    if (!(name in services)) throw new PluginTestError(`the plugin injects ${name}: give the test a service ${name}`)
  }
  const provided = new Map()
  const cleanups = []
  // Events are Cordis's own, as on the Node runtime: the plugin's listeners
  // belong to a fiber of a Context of its own, which unload disposes.
  const bus = new Context()
  let events
  const fiber = bus.plugin({ name: 'rutis-testing', apply: context => { events = context } })
  await fiber.await()
  let unloaded = false
  const ctx = {
    use(name) {
      if (!plugin.inject.includes(name)) throw new PluginTestError(`the plugin uses ${name} without declaring it in inject`)
      return strict ? cross(services[name], `service ${name}`) : services[name]
    },
    provide(name, value) {
      const shape = plugin.provides[name]
      if (shape) {
        for (const method of Object.keys(shape)) {
          if (typeof value?.[method] !== 'function') throw new PluginTestError(`${name} is declared with ${method} in provides, but the service has no such method`)
        }
      }
      const entry = { value }
      provided.set(name, entry)
      return () => {
        if (provided.get(name) === entry) provided.delete(name)
      }
    },
    effect(cleanup) {
      if (typeof cleanup !== 'function') throw new PluginTestError('effect needs a cleanup function')
      cleanups.push(cleanup)
    },
    on(name, handler) {
      if (unloaded) throw new PluginTestError('the plugin is unloaded')
      if (typeof handler !== 'function') throw new PluginTestError('on needs a listener function')
      internal(name)
      return events.on(name, handler)
    },
    emit(name, ...args) {
      internal(name)
      events.emit(name, ...args)
    },
  }
  const returned = await plugin.apply(ctx, strict ? cross(config, 'config') : config)
  if (returned !== undefined && returned !== null) {
    if (typeof returned !== 'function') throw new PluginTestError('apply must return a cleanup function or nothing')
    cleanups.push(returned)
  }
  return {
    /** The service `name` the plugin provides, as rutis sees it: its declared methods only. */
    service(name) {
      if (unloaded) throw new PluginTestError('the plugin is unloaded')
      const shape = plugin.provides[name]
      if (!shape) throw new PluginTestError(`${name} is not declared in provides, so rutis cannot use it`)
      const entry = provided.get(name)
      if (!entry) throw new PluginTestError(`the plugin does not provide ${name} (now)`)
      return shaped(name, entry.value, shape, strict)
    },
    /** The names of the services the plugin provides now. */
    provided: () => [...provided.keys()],
    /** Emit as the host would with `Process::emit`: copy arguments in strict mode and await every listener, aggregating their errors. */
    async emit(name, ...args) {
      internal(name)
      if (unloaded) throw new PluginTestError('the plugin is unloaded')
      if (!declared.has(name)) throw new PluginTestError(`event ${name} is not declared for emission from rutis`)
      const values = strict ? args.map((arg, i) => cross(arg, `event ${name} argument ${i}`)) : args
      try {
        await bus.parallel(name, ...values)
      } catch (error) {
        throw strict ? crossThrown(error, `event ${name}`) : error
      }
    },
    /** Run the cleanups, latest first; the plugin's services are withdrawn. */
    async unload() {
      if (unloaded) return
      unloaded = true
      try {
        // The listeners leave first, as on every runtime.
        await fiber.dispose()
        for (const cleanup of cleanups.splice(0).reverse()) await cleanup()
      } finally {
        provided.clear()
      }
    },
  }
}
