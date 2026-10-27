// Test a plugin without a host: give it the services it injects, call the
// services it provides, unload it.
//
//   const t = await load(plugin, { config: { city: 'Oslo' }, services: { llm } })
//   assert.equal(await t.service('weather').today(), 'sunny in Oslo')
//   assert.equal(await t.serial('weather/ask', 'Oslo'), 'sunny')
//   await t.unload()
//
// Several plugins share one event bus on a `host()`, each row with its
// scope descriptor:
//
//   const h = host({ services: { llm } })
//   const chat = await h.load(chatPlugin, { scope: 't1/s1' })
//   await h.parallel('session/created', id)
//
// Events (#464) run as on a real runtime whose listeners are all in its
// process: the five modes (emit, parallel, serial, waterfall, bail), the
// flat bus (name delivery; a scope is a descriptor), filters,
// prepend and global, emit's failures reported (`reported`), rutis/*
// refused, and unload withdrawing listeners, aborting `ctx.signal` and
// waiting for those still running. rutis/* events are the host's: the
// tool emits none.
//
// It checks what a host would: the plugin uses only the services it declares
// in `inject`, provides what it declares in `provides` with every declared
// method, and runs its cleanups on unload. A service `provides` does not
// declare is used as rutis uses it: its methods are the object's functions
// (own and inherited, not `constructor`), all async. With `strict` (the default),
// values cross between the plugin and the test as they would between
// processes: data is copied, functions and objects with behaviour pass by
// reference, sync methods return values and async ones promises; a Date,
// binary data, a Map, a Set, a bigint and the values of the SDK
// constructors cross as the runtimes send and receive them (#308: a Date
// to the millisecond, a datetime, date, time, decimal or uuid as its
// text); what works only in one process fails here too.

import { PLUGIN_API, isPlugin } from './index.mjs'
import { checkConfig, configError, schemaOf } from './config.mjs'
import { Bus, kindOf, refuseRemoved } from './events.mjs'
import { ValueError, bytesOf, checkKeys, checkText, constructedTag, instantFromText, instantText, keyOf, tagOf } from './values.mjs'

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
  if (builtins.some(type => value instanceof type) || constructedTag(value)) return false
  const prototype = Object.getPrototypeOf(value)
  if (prototype !== Object.prototype && prototype !== null) return true
  return Object.values(value).some(item => typeof item === 'function')
}

// Whether a value holds a reference or a tagged value (a Date, a Map, ...)
// anywhere: itself, or inside the arrays and plain objects it contains. A
// plain object that holds none is plain data (JSON), where undefined
// becomes null.
function holdsSpecial(value, seen = new Set()) {
  if (typeof value === 'function' || value instanceof Promise || hasBehaviour(value) || tagOf(value)) return true
  if (value === null || typeof value !== 'object' || seen.has(value)) return false
  if (!Array.isArray(value) && ![Object.prototype, null].includes(Object.getPrototypeOf(value))) return false
  seen.add(value)
  return Object.values(value).some(item => holdsSpecial(item, seen))
}

// How deep a value crossing between processes may nest, as in the
// runtimes (#279): each array or object is a level.
const MAX_DEPTH = 64

// `value` as the other process sees it, by the rules of the runtimes
// (node/rutis-runtime/src/session.mjs); `where`: what a refusal starts
// with (`llm.ask: argument 1 cannot be sent`), the same for what `value`
// holds, as the runtimes say it; `data`: inside plain data; `depth`: the
// arrays and objects around it; `name`: what a call of `value` is called
// (`llm.ask`; a function's, by default `a function`), or for an object
// with methods, what a call of each method is (by default `the method m`).
function cross(value, where, seen = new Set(), data = false, depth = 0, name = undefined) {
  if (value === undefined) return data ? null : undefined
  if (value === null) return value
  if (typeof value === 'symbol') {
    throw refused(where, `a ${typeof value} cannot cross between processes`)
  }
  if (typeof value === 'number' && !Number.isFinite(value)) {
    throw refused(where, `${value} cannot cross between processes (only finite numbers do)`)
  }
  if (typeof value === 'function') {
    const call = typeof name === 'string' ? name : 'a function'
    return (...args) => crossResult(() => value(...args.map((arg, i) => cross(arg, sending(call, `argument ${i + 1}`)))), call)
  }
  if (value instanceof Promise) return value.then(result => cross(result, where), thrown => { throw crossThrown(thrown, where) })
  const tag = tagOf(value)
  if (tag) {
    try {
      return crossTagged(value, tag, where, seen, depth)
    } catch (error) {
      throw error instanceof ValueError ? refused(where, `${error.message}`) : error
    }
  }
  if (typeof value !== 'object') return value
  // A plugin definition crosses as itself: what receives it hands it back
  // to the runtime that defined it, which loads it as it is
  // (docs/design-plugin-values-2026-10-25.md R10), so `load` takes it.
  if (isPlugin(value)) return value
  // A proxy passed back is its object again, as in the runtimes.
  if (ORIGINALS.has(value)) return ORIGINALS.get(value)
  // A service the test gives keeps its shape (`name`: how its calls are
  // named); any other object with methods crosses as a proxy (#687).
  if (hasBehaviour(value)) return typeof name === 'function' ? reference(value, name) : remoteObject(value)
  if (seen.has(value)) throw refused(where, `cyclic data cannot cross between processes`)
  if (depth >= MAX_DEPTH) throw refused(where, `a value nested deeper than ${MAX_DEPTH} levels cannot cross between processes`)
  // An Error passed as a value crosses as data: its name, message and stack.
  if (value instanceof Error && !data) {
    return { name: value.name, message: value.message, ...(typeof value.stack === 'string' ? { stack: value.stack } : {}) }
  }
  if (!Array.isArray(value) && ![Object.prototype, null].includes(Object.getPrototypeOf(value))) {
    const kind = value.constructor?.name ?? 'object'
    throw refused(where, `a ${kind} cannot cross between processes (only plain data, functions and objects with methods do)`)
  }
  seen.add(value)
  try {
    // Undefined inside a value is null, also an empty slot, as the runtimes
    // send it (#687 §七).
    const inside = (item, inData) => item === undefined ? null : cross(item, where, seen, inData, depth + 1)
    if (Array.isArray(value)) return Array.from(value, item => inside(item, data))
    const inData = data || !holdsSpecial(value)
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, inside(item, inData)]))
  } finally {
    seen.delete(value)
  }
}

// A tagged value as the other process receives it (the runtimes'
// session.mjs, values.mjs).
function crossTagged(value, tag, where, seen, depth) {
  switch (tag) {
    case 'instant': return instantFromText(instantText(value))
    case 'bytes': return new Uint8Array(bytesOf(value))
    case 'bigint': return BigInt(checkText(tag, String(value)))
    case 'map': case 'set': {
      if (seen.has(value)) throw refused(where, `cyclic data cannot cross between processes`)
      if (depth >= MAX_DEPTH) throw refused(where, `a value nested deeper than ${MAX_DEPTH} levels cannot cross between processes`)
      const what = tag === 'map' ? 'map key' : 'set element'
      const entries = tag === 'map' ? [...value] : [...value].map(element => [element])
      const keys = entries.map(([key]) => keyOf(key))
      checkKeys(keys, what)
      seen.add(value)
      let made
      try {
        const item = item => cross(item, where, seen, false, depth + 1)
        made = tag === 'map'
          // Undefined in a map is null, as in data (design V10).
          ? new Map(entries.map(([key, entry], i) => [item(key), entry === undefined ? null : item(entry)]))
          : new Set(entries.map(([element]) => item(element)))
      } finally {
        seen.delete(value)
      }
      // Received, a datetime, date, ... is its text: no two keys may meet.
      checkKeys(keys, what, made)
      return made
    }
    default: return checkText(tag, value.value)
  }
}

// What cannot be sent says which part of which call, as the runtimes do:
// `llm.ask: argument 1 cannot be sent: …`.
// It carries the code and data of the runtimes' refusal (`bad-value`,
// `{ part, argument? }`), which `refused` gives the error.
function sending(where, what) {
  const argument = /^argument (\d+)$/.exec(what)
  return Object.assign(new String(`${where}: ${what} cannot be sent`), { data: argument ? { part: 'argument', argument: Number(argument[1]) } : { part: 'result' } })
}
function refused(where, reason) {
  const error = new PluginTestError(`${where}: ${reason}`)
  return where?.data ? Object.assign(error, { code: 'bad-value', data: { ...where.data } }) : error
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
    ? result.then(value => cross(value, sending(where, 'the result')), thrown => { throw crossThrown(thrown, where) })
    : cross(result, sending(where, 'the result'))
}

// A thrown value as the other process receives it: an Error is copied with
// its name, message, cause, (AggregateError) errors, a string `code` and
// its `data` (when it is data that can cross), as an Error of the same
// built-in class or a plain Error, with the receiving side's own stack, as
// the runtimes do (§4.4); its other fields stay behind. Other thrown values
// are copied.
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
    Object.assign(copy, { name: thrown.name })
    if (typeof Error.captureStackTrace === 'function') Error.captureStackTrace(copy, crossThrown)
    copies.set(thrown, copy)
    if (typeof thrown.code === 'string') copy.code = thrown.code
    if (Object.hasOwn(thrown, 'data')) {
      try { copy.data = cross(thrown.data, 'data', new Set(), true) } catch { /* data that cannot cross stays behind, as in the runtimes */ }
    }
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
// `method(m)`: what a call of the method `m` is called, as the runtimes
// say it: `llm.ask` for a service's, `the method ask` for an object's.
function reference(target, method = property => `the method ${property}`) {
  return new Proxy(target, {
    get(object, property) {
      const value = Reflect.get(object, property, object)
      return typeof value === 'function'
        ? cross(value.bind(object), undefined, new Set(), false, 0, method(String(property)))
        : cross(value, sending(`the property ${String(property)}`, 'the result'))
    },
  })
}

// A received object with methods, as the runtimes give it (#687 §四):
// every name read is a method, an asynchronous call of the object's; it has
// no fields, shows as `[remote object]`, and a method it lacks fails with
// `no-method` when called. One proxy per object.
const PROXIES = new WeakMap()
const ORIGINALS = new WeakMap()
const NOT_METHODS = new Set(['then', 'toJSON', 'toString', 'valueOf'])
const INSPECT = Symbol.for('nodejs.util.inspect.custom')
function remoteObject(object) {
  let proxy = PROXIES.get(object)
  if (proxy) return proxy
  const show = () => '[remote object]'
  const methods = new Map()
  const method = name => {
    let call = methods.get(name)
    if (call) return call
    const where = `the method ${name}`
    call = (...args) => {
      try {
        const own = object[name]
        if (typeof own !== 'function') throw noMethod(object, name)
        const crossed = args.map((arg, i) => cross(arg, sending(where, `argument ${i + 1}`)))
        return Promise.resolve(own.apply(object, crossed)).then(value => cross(value, sending(where, 'the result')), thrown => { throw crossThrown(thrown, where) })
      } catch (thrown) {
        return Promise.reject(crossThrown(thrown, where))
      }
    }
    methods.set(name, call)
    return call
  }
  const target = Object.create(null)
  Object.defineProperty(target, INSPECT, { value: show })
  proxy = new Proxy(target, {
    get: (_target, name) => {
      if (name === INSPECT || name === 'toString' || name === Symbol.toPrimitive) return show
      if (typeof name === 'symbol' || NOT_METHODS.has(name)) return undefined
      return method(name)
    },
    has: (_target, name) => name === INSPECT || (typeof name === 'string' && !NOT_METHODS.has(name)),
    set: () => false,
    defineProperty: () => false,
    deleteProperty: () => false,
  })
  PROXIES.set(object, proxy)
  ORIGINALS.set(proxy, object)
  return proxy
}

// A method `object` lacks, as the runtimes say it (#687 §八): code
// `no-method`, data `{ object, method, methods }`.
function noMethod(object, name) {
  const prototype = Object.getPrototypeOf(object)
  const className = prototype && prototype !== Object.prototype ? prototype.constructor?.name : undefined
  const owner = typeof className === 'string' && className && className !== 'Object' ? className : null
  const methods = Object.keys(inferred(object)).sort()
  const why = name in Object(object) && typeof object[name] !== 'function'
    ? `${name} is a field; objects passed by reference expose only their methods`
    : methods.length ? `methods: ${methods.join(', ')}` : 'no methods'
  return coded(`${owner ?? 'the object'} has no method ${name} (${why})`, 'no-method', { object: owner, method: name, methods })
}

// The methods of a service no declaration describes, as the runtimes infer
// them: its own and inherited functions, up to Object's, not the
// constructor; a property that is not a function hides an inherited
// function of the same name. All async.
function inferred(object) {
  const shape = {}
  if (object === null || (typeof object !== 'object' && typeof object !== 'function')) return shape
  const seen = new Set()
  for (let level = object; level != null && level !== Object.prototype && level !== Function.prototype; level = Object.getPrototypeOf(level)) {
    for (const name of Object.getOwnPropertyNames(level)) {
      if (seen.has(name)) continue
      seen.add(name)
      if (name !== 'constructor' && typeof Object.getOwnPropertyDescriptor(level, name)?.value === 'function') shape[name] = 'async'
    }
  }
  return shape
}

// A service seen through its declared shape: only declared methods, sync
// ones returning values, async ones promises.
function shaped(name, object, shape, strict) {
  const service = {}
  for (const [method, kind] of Object.entries(shape)) {
    service[method] = (...args) => {
      const where = `${name}.${method}`
      const crossed = strict ? args.map((arg, i) => cross(arg, sending(where, `argument ${i + 1}`))) : args
      let result
      try {
        result = object[method](...crossed)
      } catch (thrown) {
        throw strict ? crossThrown(thrown, where) : thrown
      }
      if (kind === 'sync') {
        if (result instanceof Promise) throw new PluginTestError(`${where} is declared sync but returned a promise: declare it 'async'`)
        return strict ? cross(result, sending(where, 'the result')) : result
      }
      if (!strict) return Promise.resolve(result)
      return Promise.resolve(result).then(value => cross(value, sending(where, 'the result')), thrown => { throw crossThrown(thrown, where) })
    }
  }
  return service
}

// A refusal as the runtimes make it (§4.3): their text, code and data, on
// the testing tool's own error.
const coded = (message, code, data) => Object.assign(new PluginTestError(message), { code, data })

// A service the test gives, as the runtimes give a host's: a method it does
// not have fails when called, naming those it has (the runtimes' words).
function withoutMissing(name, object, used) {
  if (used === null || typeof used !== 'object') return used
  const passthrough = new Set(['then', 'toJSON', 'constructor'])
  return new Proxy(used, {
    get(target, property, receiver) {
      if (typeof property !== 'string' || property in object || passthrough.has(property)) return Reflect.get(target, property, receiver)
      const methods = Object.keys(inferred(object)).sort()
      return () => { throw coded(`${name} has no method ${property} (${methods.length ? `methods: ${methods.join(', ')}` : 'no methods'})`, 'no-method', { service: name, method: property, methods }) }
    },
  })
}

// As on the real runtimes, internal/* events belong to the runtime. The SDK
// is a package of its own, so it keeps its own copy of the wording the
// runtime uses (`node/rutis-runtime/src/leaf.mjs`).
const internalMessage = name => `event ${name} is internal to the runtime`

// A name is a string: Cordis's `emit` takes an object, null or function first
// argument as `this`, so any other type would let `emit(null, 'internal/service')`
// past the check.
function internal(name) {
  if (typeof name !== 'string') throw new TypeError(`an event name is a string, not ${name === null ? 'null' : typeof name}`)
  if (name.startsWith('internal/')) throw new PluginTestError(internalMessage(name))
}

// A scope path (`t1/s1`; `root` or nothing for the root) as the chain a
// host gives a row (protocol §3): each instance's id is its path, its group
// `groups[path]` or the path's last part.
function chainOf(path, groups) {
  if (path === undefined || path === null || path === '' || path === 'root') return []
  if (typeof path !== 'string') throw new PluginTestError('a scope is a path such as t1/s1')
  const parts = path.split('/')
  if (parts.some(part => part === '')) throw new PluginTestError(`invalid scope path ${path}`)
  return parts.map((part, index) => {
    const id = parts.slice(0, index + 1).join('/')
    return { id, group: groups?.[id] ?? part }
  })
}

// The five ways to emit and `filter`, on `bus`, from `row`.
// `crossing`: values cross between the emitter and the listeners (the test
// emitting, strict), as from another process. `errors`: what the
// listeners throw crosses (a plugin emitting, strict): a listener may be
// in another process, where an error arrives as an Error of a built-in
// class or a plain Error with the thrower's `name` (not its class), so a
// plugin matching errors by class fails here as it would there.
function emitters(bus, row, crossing, keep, errors = crossing) {
  const where = name => `event ${name}`
  const inbound = (name, values) => crossing ? values.map((value, i) => cross(value, sending(where(name), `argument ${i + 1}`))) : values
  const outbound = (name, value) => {
    if (value instanceof Promise) {
      if (crossing) return value.then(result => cross(result, where(name)), thrown => { throw crossThrown(thrown, where(name)) })
      return errors ? value.catch(thrown => { throw crossThrown(thrown, where(name)) }) : value
    }
    return crossing ? cross(value, where(name)) : value
  }
  const dispatch = (mode, name, values, terminal) => {
    internal(name)
    let result
    try {
      result = bus.dispatch(mode, name, inbound(name, values), { row, keep, terminal: crossing && terminal ? cross(terminal, `${where(name)} terminal`) : terminal })
    } catch (thrown) {
      throw errors ? crossThrown(thrown, where(name)) : thrown
    }
    return outbound(name, result)
  }
  // Every argument of `waterfall` is the event's; a terminal is given with
  // `waterfall.with(terminal)(name, ...args)`.
  const waterfall = terminal => (name, ...values) => dispatch('waterfall', name, values, terminal)
  return {
    emit: (name, ...values) => { dispatch('emit', name, values) },
    parallel: (name, ...values) => dispatch('parallel', name, values),
    serial: (name, ...values) => dispatch('serial', name, values),
    waterfall: Object.assign(waterfall(undefined), {
      with: terminal => {
        if (typeof terminal !== 'function') throw new PluginTestError('waterfall.with needs a terminal function')
        return waterfall(terminal)
      },
    }),
    bail: (name, ...values) => dispatch('bail', name, values),
    filter: predicate => {
      if (typeof predicate !== 'function') throw new PluginTestError('filter needs a predicate function')
      const { filter: _, ...five } = emitters(bus, row, crossing, descriptor => predicate(crossing ? cross(descriptor, 'a listener descriptor') : descriptor), errors)
      return five
    },
  }
}

/**
 * A host for plugins in one process, as a real runtime is (#464): the rows
 * share one event bus, always dispatched here (the fast path), with the
 * five modes, scopes, filters, unload waiting and cancellation. `services`
 * are given to every row that injects them; `scopes` names the group of a
 * scope path (`{ t1: 'tenant' }`); `deadline` is how long an unloading
 * row's listeners are waited for (ms; by default RUTIS_SHUTDOWN_TIMEOUT
 * seconds, else 10 s, as the runtimes); `runtime` is what descriptors call
 * the runtime (`node`, or `bun` under Bun, by default).
 */
export function host({ services = {}, strict = true, scopes = {}, deadline = shutdownDeadline(), runtime = defaultRuntime(), ...rest } = {}) {
  refuse(rest)
  const reported = []
  const bus = new Bus({ runtime, report: (line, details = {}) => { reported.push({ line, ...details }) } })
  const rows = new Map()
  // The test's own listeners (`on`), withdrawn when the host unloads.
  const listening = []
  let counter = 0
  let closed = false
  // Where the service `name` comes from: the test (`own`, then `services`),
  // or another row here.
  const lookup = (name, own) => {
    if (Object.hasOwn(own, name)) return { test: true, value: own[name] }
    if (Object.hasOwn(services, name)) return { test: true, value: services[name] }
    for (const row of rows.values()) {
      const entry = row.provided.get(name)
      if (entry) return { test: false, value: entry.value }
    }
    return undefined
  }
  const self = {
    /** The failures `emit` reported (§5.6): `{ line, event, row, error }`. */
    get reported() { return [...reported] },
    /** Load `plugin` as a row (key `row`, scope path `scope`), with `config` and its own `services`. */
    async load(plugin, { config, services: own = {}, scope, row: key, ...more } = {}) {
      refuse(more)
      if (closed) throw new PluginTestError('the host is unloaded')
      plugin = isPlugin(plugin) ? plugin : plugin?.default
      if (!isPlugin(plugin)) throw new PluginTestError('load needs a plugin made with definePlugin')
      if (plugin.api > PLUGIN_API) {
        throw new PluginTestError(`the plugin needs plugin API ${plugin.api}; this SDK supports ${PLUGIN_API}`)
      }
      key ??= `row${++counter}`
      if (rows.has(key)) throw new PluginTestError(`row ${key} is already loaded`)
      for (const name of plugin.inject) {
        if (lookup(name, own) === undefined) throw new PluginTestError(`the plugin injects ${name}: give the test a service ${name}`)
      }
      const row = { key, plugin, chain: chainOf(scope, scopes), provided: new Map(), cleanups: [], abort: new AbortController(), unloaded: false }
      rows.set(key, row)
      bus.nameRow(key, key)
      const ctx = {
        use(name) {
          if (!plugin.inject.includes(name)) throw coded(`service ${name} is not available to this plugin, which does not declare it in inject: add '${name}' to its inject`, 'not-declared', { service: name })
          const found = lookup(name, own)
          if (found === undefined) throw coded(`service ${name} is not available`, 'unavailable', { service: name })
          // A service another row here provides is the object itself.
          if (!found.test) return found.value
          return strict ? withoutMissing(name, found.value, cross(found.value, `service ${name}`, new Set(), false, 0, method => `${name}.${method}`)) : found.value
        },
        provide(name, value) {
          const shape = plugin.provides[name]
          if (shape) {
            for (const method of Object.keys(shape)) {
              if (typeof value?.[method] !== 'function') throw new PluginTestError(`${name} is declared with ${method} in provides, but the service has no such method`)
            }
          }
          const entry = { value }
          row.provided.set(name, entry)
          return () => {
            if (row.provided.get(name) === entry) row.provided.delete(name)
          }
        },
        effect(cleanup) {
          if (typeof cleanup !== 'function') throw new PluginTestError('effect needs a cleanup function')
          row.cleanups.push(cleanup)
        },
        /** Aborted when the row unloads, while its listeners are waited for. */
        get signal() { return row.abort.signal },
        on(name, listener, options) {
          if (row.unloaded) throw new PluginTestError('the plugin is unloaded')
          if (typeof listener !== 'function') throw new PluginTestError('on needs a listener function')
          internal(name)
          // Named by the plugin's declared name, else its row's id (§4.2).
          const entry = bus.on({ name, callback: listener, prepend: options?.prepend, global: options?.global, row: key, plugin: plugin.name ?? key, scope: row.chain })
          return () => { bus.off(entry) }
        },
        ...emitters(bus, key, false, undefined, strict),
      }
      row.apply = config => applyRow(row, ctx, config)
      await row.apply(config)
      return loaded(row)
    },
    /** Unload every row, latest first. */
    async unload() {
      closed = true
      for (const row of [...rows.values()].reverse()) await unloadRow(row)
      for (const entry of listening.splice(0)) bus.off(entry)
    },
    // The five ways to emit and `filter`, as the test: values cross as from
    // another process (strict).
    ...emitters(bus, null, strict),
  }

  // Run the plugin's apply for `row` with `config`; on failure, unload the row.
  async function applyRow(row, ctx, config) {
    const { plugin } = row
    try {
      // As a runtime loads a row: defaults filled, the config validated
      // against the plugin's schema (config schema design §2.2); extra keys
      // warned about on standard error, as the host's loader would.
      // The config crosses first, as it reaches a runtime.
      if (strict && config !== undefined) config = cross(config, 'config')
      const schema = schemaOf(plugin.config)
      if (schema !== undefined) {
        const checked = checkConfig(schema, config)
        // Once per row and config value, as the host's loader warns.
        let seen
        try { seen = JSON.stringify(config, (_, value) => (typeof value === 'bigint' ? `${value}n` : value)) ?? '' } catch {}
        row.warned ??= new Set()
        if (checked.warnings.length && (seen === undefined || !row.warned.has(seen))) {
          if (seen !== undefined) row.warned.add(seen)
          for (const warning of checked.warnings) process.stderr.write(`${row.key}: config: ${warning.message}\n`)
        }
        if (checked.errors.length) throw configError(checked.errors)
        config = checked.config
      } else if (config === undefined) {
        // No schema: what it is given, `{}` when nothing (as before).
        config = strict ? cross({}, 'config') : {}
      }
      const returned = await plugin.apply(ctx, config)
      if (returned !== undefined && returned !== null) {
        if (typeof returned !== 'function') throw new PluginTestError('apply must return a cleanup function or nothing')
        row.cleanups.push(returned)
      }
    } catch (error) {
      await unloadRow(row).catch(() => {})
      throw error
    }
  }

  // Withdraw the row's listeners, then its services; signal its running
  // listeners and wait for them (at most `deadline`); then run its
  // cleanups, latest first (protocol §7.1).
  async function unloadRow(row) {
    if (row.unloaded) return
    row.unloaded = true
    bus.withdraw(row.key)
    row.provided.clear()
    row.abort.abort(new DOMException('the row is unloading', 'AbortError'))
    try {
      await bus.settle(row.key, deadline)
      for (const cleanup of row.cleanups.splice(0).reverse()) await cleanup()
    } finally {
      rows.delete(row.key)
    }
  }

  function loaded(row) {
    const { plugin } = row
    // This row's test listeners: withdrawn when it unloads (not on update).
    const mine = []
    const withdraw = entry => {
      for (const list of [mine, listening]) {
        const at = list.indexOf(entry)
        if (at >= 0) list.splice(at, 1)
      }
      bus.off(entry)
    }
    return {
      /** The row's key. */
      row: row.key,
      /** The service `name` the plugin provides, as rutis sees it: its declared methods, or, undeclared, the object's methods, all async. */
      service(name) {
        if (row.unloaded) throw new PluginTestError('the plugin is unloaded')
        const entry = row.provided.get(name)
        if (!entry) throw new PluginTestError(`the plugin does not provide ${name} (now)`)
        return shaped(name, entry.value, plugin.provides[name] ?? inferred(entry.value), strict)
      },
      /** The methods rutis sees on the service `name`: `{ method: 'sync' | 'async' }`. */
      methods(name) {
        const entry = row.provided.get(name)
        if (!entry) throw new PluginTestError(`the plugin does not provide ${name} (now)`)
        return { ...(plugin.provides[name] ?? inferred(entry.value)) }
      },
      /** The names of the services the plugin provides now. */
      provided: () => [...row.provided.keys()],
      /** The failures `emit` reported, on the whole host. */
      get reported() { return self.reported },
      /** The host this row is loaded on. */
      host: self,
      // The five ways to emit and `filter`, as the test: values cross as
      // from another process (strict).
      ...emitters(bus, null, strict),
      /**
       * Listen to the event `name` as the host does, at the root scope, until
       * the returned function is called or the row unloads: the plugin's
       * emissions of `name` reach `listener`, and so do the test's own.
       * Strict, its arguments and what it returns cross as from another
       * process. Options as `ctx.on`: `prepend`, `global`.
       */
      on(name, listener, options) {
        if (row.unloaded) throw new PluginTestError('the plugin is unloaded')
        if (typeof listener !== 'function') throw new PluginTestError('on needs a listener function')
        internal(name)
        const callback = strict ? cross(listener, undefined, new Set(), false, 0, `listener of ${name}`) : listener
        const entry = bus.on({ name, callback, kind: kindOf(listener), prepend: options?.prepend, global: options?.global, row: null, plugin: 'test', scope: [] })
        mine.push(entry)
        listening.push(entry)
        return () => { withdraw(entry) }
      },
      /**
       * Change the row's config, as a host's `rows.update`: the row unloads
       * (as `unload`) and applies again with `config` under the same key, so
       * its listeners register again, after every existing one (§4.3).
       */
      async update(config) {
        if (row.unloaded) throw new PluginTestError('the plugin is unloaded')
        if (closed) throw new PluginTestError('the host is unloaded')
        await unloadRow(row)
        Object.assign(row, { provided: new Map(), cleanups: [], abort: new AbortController(), unloaded: false })
        rows.set(row.key, row)
        bus.nameRow(row.key, row.key)
        await row.apply(config)
      },
      /** Withdraw its listeners and services, wait for its running listeners (signalled), then run its cleanups, latest first. */
      async unload() {
        if (row.unloaded) return
        await unloadRow(row)
        for (const entry of [...mine]) withdraw(entry)
      },
    }
  }

  return self
}

// What a 0.12 test passed and 0.13 removed (#464).
function refuse(options) {
  try { refuseRemoved(options) } catch (error) { throw new PluginTestError(error.message) }
}

/**
 * Load `plugin` (what definePlugin returns, or a module whose default export
 * it is) with `config` and the services in `services`, as one row of a host
 * of its own (`host()`), in the scope path `scope`.
 */
export async function load(plugin, { config, services = {}, strict = true, scope, deadline, runtime, ...rest } = {}) {
  refuse(rest)
  return host({ services, strict, deadline, runtime }).load(plugin, { config, scope })
}

// How long an unloading row's listeners are waited for, in ms: as the
// runtimes, RUTIS_SHUTDOWN_TIMEOUT seconds, 10 by default.
function shutdownDeadline() {
  const seconds = Number(globalThis.process?.env?.RUTIS_SHUTDOWN_TIMEOUT)
  return Number.isFinite(seconds) && seconds > 0 ? seconds * 1000 : 10_000
}

// The runtime's name in descriptors, as a host names its runtimes by
// default: the language's.
function defaultRuntime() {
  return typeof globalThis.Bun === 'undefined' ? 'node' : 'bun'
}
